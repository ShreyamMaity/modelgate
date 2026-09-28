import re
import sys
from pathlib import Path

from nltk.corpus import names
from wordfreq import top_n_list

TOP = 25000
DICT = "/usr/share/dict/american-english-large"
ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "src/pii/common_words.txt"

INDIAN = """
Aarti Abhishek Aditya Ajay Akash Akshay Alok Aman Amit Amitabh Anand Anil Anita Anjali Ankit Ankita
Anupam Anuradha Anurag Arun Aruna Arvind Asha Ashok Ashwin Bharat Bhavna Chetan Chitra Deepa Deepak
Deepika Dev Devendra Dinesh Divya Ganesh Gaurav Geeta Girish Gita Gopal Govind Hari Harish Hema Indira
Ishaan Jaya Jyoti Kajal Kamal Kamala Karan Kavita Kavya Kiran Krishna Kumar Lakshmi Lata Madhu Mahesh
Manish Manoj Meena Meera Mohan Mukesh Nandini Naresh Naveen Neha Nidhi Nikhil Nisha Nitin Pankaj Pooja
Prakash Pradeep Pramod Prasad Prashant Preeti Priya Priyanka Rachna Radha Raj Raja Rajesh Rajiv Raju
Rakesh Ram Rama Ramesh Rani Ravi Rekha Ritu Rohit Sachin Sadhana Sagar Sanjay Santosh Sapna Sarita
Satish Seema Shanti Sharmila Shiva Shobha Shyam Sita Smita Sneha Sonia Suman Sunil Sunita Suraj Suresh
Sushma Swati Uma Usha Varun Vijay Vikas Vinod Vishal Yash Yogesh Sunny Sweety Pinky Bunty Babli Guddu
Chintu Pappu Munna Heera Moti Sona Chanda Kali Durga Lalit Mani Ratan Roshan Sunder Sundar Tara Jeet
Veer Noor Iqbal Imran Irfan Salman Farhan Ayesha Fatima Zainab Arif Asif Tariq Zubair Karim Rahim
Rehman Sharma Verma Gupta Singh Das Dutta Ghosh Bose Sen Roy Rao Reddy Nair Menon Pillai Iyer Iyengar
Patel Shah Mehta Joshi Kulkarni Desai Jain Agarwal Bansal Mishra Tiwari Pandey Yadav Chauhan Rathore
Thakur Malhotra Kapoor Khanna Chopra Bhatia Sethi Grover Arora Gill Sandhu Dhillon Sidhu Banerjee
Chatterjee Mukherjee Bhattacharya Chakraborty Ganguly Majumdar Sarkar Mondal Saha Paul Khan Ahmed Ali
Hussain Qureshi Siddiqui Ansari Sheikh Naidu Chettiar Gowda Hegde Shetty Kamath Pai Bhat Acharya
Sastry Murthy Varma Choudhary Chaudhary Saxena Srivastava Shukla Dubey Trivedi Dwivedi Chaturvedi
Bhardwaj Tripathi Sinha Jha Lal Chand Nath Pal Dey Bera Jana Sahu Behera Mohanty Patnaik Swain Nayak
Barua Bora Gogoi Hazarika Kalita
"""


def pool(src, name):
    m = re.search(r"const " + name + r": &\[&str\] = &\[(.*?)\];", src, re.S)
    return re.findall(r'"([^"]+)"', m.group(1)) if m else []


def main():
    lower, proper = set(), set()
    for line in open(DICT, encoding="utf-8", errors="ignore"):
        w = line.strip()
        if not re.fullmatch(r"[A-Za-z]+", w):
            continue
        if w.islower():
            lower.add(w)
        else:
            proper.add(w.lower())
    src = (ROOT / "src/pii/surrogate.rs").read_text(encoding="utf-8")
    given = {n.lower() for n in names.words()}
    given |= {w.lower() for w in INDIAN.split()}
    given |= {w.lower() for n in pool(src, "GIVEN") + pool(src, "FAMILY") for w in n.split()}
    words = {w for w in top_n_list("en", TOP) if re.fullmatch(r"[a-z]{3,}", w)}
    out = [w + ("*" if w in proper else "") for w in sorted(words & lower - given)]
    OUT.write_text("\n".join(out) + "\n", encoding="utf-8")
    print(len(out), file=sys.stderr)


if __name__ == "__main__":
    main()
